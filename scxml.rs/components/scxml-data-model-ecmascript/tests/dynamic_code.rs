//! `eval` and the `Function` constructors compile runtime-built source.
//!
//! B.2: "Implementations that support this value MUST support the third
//! edition of ECMAScript [ECMASCRIPT-262]", whose global object has `eval`
//! (15.1.2.1) and `Function` (15.3). Every program text handed to Boa must
//! pass `check_nesting` (Boa's parser recurses natively), so the host hook
//! that ECMA-262 consults before compiling a runtime string
//! (`HostEnsureCanCompileStrings`) applies the same bound: a string within
//! it compiles, and a deeper one is refused with a catchable `TypeError`
//! instead of overflowing the native stack.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{DataValue, Event, EventName, FiniteNumber};

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
            r#"{"cnt":1}"#,
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("the data model initializes");
    model
}

fn event() -> Event {
    Event::external(EventName::new("e").expect("static event name is valid"))
}

fn evaluate(expression: &str) -> Result<DataValue, DataModelError> {
    model().evaluate_value(expression, &event(), &mut NoStates)
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("finite"))
}

/// Every syntactic form whose prototype chain reaches one of the four
/// dynamic-function constructors.
const FUNCTION_KINDS: &[(&str, &str)] = &[
    ("arrow", "(() => 0)"),
    ("function expression", "(function () {})"),
    ("method", "({ m() {} }).m"),
    ("class", "(class {})"),
    ("bound function", "(function () {}).bind(null)"),
    ("generator", "(function* () {})"),
    ("async function", "(async function () {})"),
    ("async arrow", "(async () => 0)"),
    ("async generator", "(async function* () {})"),
];

#[test]
fn eval_and_function_are_the_standard_globals() {
    for (expression, expected) in [
        ("eval('1 + 2')", number(3.0)),
        ("(0, eval)('cnt + 1')", number(2.0)),
        (
            "(function () { var cnt = 10; return eval('cnt + 1'); })()",
            number(11.0),
        ),
        ("Function('a', 'b', 'return a + b')(2, 3)", number(5.0)),
        (
            "new Function('return this')() === globalThis",
            DataValue::Bool(true),
        ),
        (
            "typeof eval === 'function' && typeof Function === 'function'",
            DataValue::Bool(true),
        ),
        ("(() => 0).constructor === Function", DataValue::Bool(true)),
        ("eval(7)", number(7.0)),
    ] {
        assert_eq!(evaluate(expression), Ok(expected), "{expression}");
    }
}

/// ECMA-262 `CreateDynamicFunction` parses the parameters and the body each
/// on its own, and the created function's source text is
/// `function anonymous(<parameters>\n) {\n<body>\n}`.
#[test]
fn function_parses_its_parameters_and_body_on_their_own() {
    assert_eq!(
        evaluate("String(new Function('a', 'b', 'return a + b'))"),
        Ok(DataValue::String(
            "function anonymous(a,b\n) {\nreturn a + b\n}".into()
        ))
    );
    for (parameters, body) in [
        ("a) {}; (function (", "return 1"),
        ("/*", "*/ a) { return a }"),
        ("a", "}, function () {"),
        ("a", "} + function () {"),
    ] {
        assert_eq!(
            evaluate(&format!(
                "(() => {{ try {{ Function({parameters:?}, {body:?}); return 'compiled'; }} \
                 catch (error) {{ return error.name; }} }})()"
            )),
            Ok(DataValue::String("SyntaxError".into())),
            "{parameters:?} {body:?}"
        );
    }
}

#[test]
fn every_function_constructor_compiles_strings() {
    for (name, kind) in FUNCTION_KINDS {
        for via in [
            "f.constructor('return 1')",
            "new f.constructor('return 1')",
            "Object.getPrototypeOf(f).constructor('return 1')",
            "Reflect.construct(f.constructor, ['return 1'])",
        ] {
            assert_eq!(
                evaluate(&format!(
                    "(() => {{ const f = {kind}; {via}; return 'compiled'; }})()"
                )),
                Ok(DataValue::String("compiled".into())),
                "{name}: {via}"
            );
        }
    }
}

#[test]
fn deep_dynamic_source_fails_closed_without_native_stack_overflow() {
    for payload in [
        "eval('[' .repeat(40000))",
        "(0, eval)('(' .repeat(40000) + '1' + ')'.repeat(40000))",
        "Function('return ' + '['.repeat(40000))()",
        "(()=>0).constructor('return ' + '('.repeat(40000) + '1' + ')'.repeat(20000))()",
        "(function*(){}).constructor('return ' + '['.repeat(40000))().next()",
        "Object.getPrototypeOf(async function(){}).constructor('return ' + '['.repeat(40000))",
        "Object.getPrototypeOf(async function*(){}).constructor('return ' + '['.repeat(40000))",
    ] {
        assert_eq!(
            evaluate(payload),
            Err(DataModelError::Execution),
            "{payload}"
        );
        assert_eq!(
            evaluate(&format!(
                "(() => {{ try {{ {payload}; return 'compiled'; }} \
                 catch (error) {{ return error.name; }} }})()"
            )),
            Ok(DataValue::String("TypeError".into())),
            "{payload} is catchable"
        );
    }
}

#[test]
fn scripts_and_their_functions_compile_strings() {
    let mut model = model();
    let host = &mut NoStates;
    model
        .execute(
            "eval('var viaEval = cnt + 4'); var made = new Function('x', 'return x * 3'); \
             function later() { return (() => 0).constructor('return 7')(); }",
            &event(),
            host,
        )
        .expect("the script compiles its strings");
    assert_eq!(
        model.evaluate_value("[viaEval, made(2), later()]", &event(), host),
        Ok(DataValue::JsonArray("[5,6,7]".into()))
    );
}

#[test]
fn no_host_timer_is_reachable() {
    assert_eq!(
        evaluate(
            "[typeof globalThis.setTimeout, typeof globalThis.setInterval, \
             typeof globalThis.queueMicrotask, typeof globalThis.ShadowRealm]"
        ),
        Ok(DataValue::JsonArray(
            "[\"undefined\",\"undefined\",\"undefined\",\"undefined\"]".into()
        ))
    );
}

#[test]
fn ordinary_functions_still_work() {
    assert_eq!(
        evaluate(
            "(() => { function* g() { yield 2; } const add = (a, b) => a + b; \
             class C { m() { return 3; } } \
             return add(g().next().value, new C().m()); })()"
        ),
        Ok(number(5.0))
    );
}
