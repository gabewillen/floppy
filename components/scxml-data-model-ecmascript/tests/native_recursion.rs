//! Hostile-input proof that deeply nested values cannot drive Boa's native
//! (Rust) recursion past the host stack. Before the guards, each expression
//! below aborted the whole process with `fatal runtime error: stack overflow`.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{DataValue, Event, EventName};

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn evaluate(expression: &str) -> Result<DataValue, DataModelError> {
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
    model.evaluate_value(
        expression,
        &Event::external(EventName::new("e").expect("static event name is valid")),
        &mut NoStates,
    )
}

/// Builds a 20 000-deep nested array inside the loop-iteration limit and
/// applies `operation` to it, reporting a guard `RangeError` as `rejected`.
fn deep(operation: &str) -> Result<DataValue, DataModelError> {
    evaluate(&format!(
        "(() => {{ let a = []; for (let i = 0; i < 20000; i++) a = [a]; \
         try {{ {operation}; return 'completed'; }} \
         catch (error) {{ return error instanceof RangeError ? 'rejected' : 'other:' + error.name; }} }})()"
    ))
}

#[test]
fn deep_json_stringify_is_rejected_without_native_stack_overflow() {
    assert_eq!(
        deep("JSON.stringify(a)"),
        Ok(DataValue::String("rejected".into()))
    );
    assert_eq!(
        deep("JSON.stringify(a, null, 2)"),
        Ok(DataValue::String("rejected".into()))
    );
}

#[test]
fn deep_array_join_and_to_string_are_rejected_without_native_stack_overflow() {
    assert_eq!(deep("String(a)"), Ok(DataValue::String("rejected".into())));
    assert_eq!(deep("a.join()"), Ok(DataValue::String("rejected".into())));
    assert_eq!(
        deep("a.toLocaleString()"),
        Ok(DataValue::String("rejected".into()))
    );
}

#[test]
fn deep_array_flat_is_rejected_without_native_stack_overflow() {
    assert_eq!(
        deep("a.flat(Infinity)"),
        Ok(DataValue::String("rejected".into()))
    );
}

#[test]
fn deep_values_returned_to_the_host_fail_closed() {
    assert_eq!(
        evaluate("(() => { let a = []; for (let i = 0; i < 20000; i++) a = [a]; return a; })()"),
        Err(DataModelError::UnsupportedValue)
    );
}

#[test]
fn deep_json_parse_is_rejected_before_boa_parses_it() {
    // `JSON.parse` runs Boa's recursive parser on runtime text, which the
    // structural pre-parse guard never sees.
    let parse = |depth: usize| {
        evaluate(&format!(
            "(() => {{ try {{ JSON.parse('['.repeat({depth}) + ']'.repeat({depth})); return 'completed'; }} \
             catch (error) {{ return error instanceof RangeError ? 'rejected' : 'other:' + error.name; }} }})()"
        ))
    };
    assert_eq!(parse(100), Ok(DataValue::String("rejected".into())));
    assert_eq!(
        parse(scxml_data_model_ecmascript::MAX_NESTING_DEPTH + 1),
        Ok(DataValue::String("rejected".into()))
    );
    assert_eq!(
        parse(scxml_data_model_ecmascript::MAX_NESTING_DEPTH),
        Ok(DataValue::String("completed".into()))
    );
    assert_eq!(
        evaluate("JSON.parse('{\"a\": [1, \"]]]\"]}').a[1]"),
        Ok(DataValue::String("]]]".into()))
    );
}

#[test]
fn deep_portable_values_are_declined_at_ingress() {
    // Portable JSON values are re-parsed by Boa on every context build, so
    // their nesting is bounded where they enter the model.
    let deep = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let limit = scxml_data_model_ecmascript::MAX_NESTING_DEPTH;
    let mut model = model();
    assert_eq!(
        model.initialize(
            &format!("{{\"v\":{}}}", deep(limit + 1)),
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        ),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.assign("x", &DataValue::JsonArray(deep(limit + 1))),
        Err(DataModelError::InvalidInput)
    );
}

#[test]
fn expressions_the_validator_declines_are_rejected_at_runtime() {
    // The validator's LIMIT003 fragment (tests/ui/limit003-*) is over the
    // shared nesting guard, so evaluation fails closed with a typed error;
    // this is why LIMIT003 is an error rather than a warning.
    let expression = format!("{}1{}", "(".repeat(20), ")".repeat(20));
    assert_eq!(evaluate(&expression), Err(DataModelError::InvalidInput));
    // The byte bound applies to wasm builds only; the host's component tests
    // check it through the shipped data model.
}

#[test]
fn proxy_chains_are_unavailable() {
    // Proxy chains recurse natively on every property access; the portable
    // data model does not need Proxy, so it is removed like eval/Function.
    assert_eq!(
        evaluate("typeof Proxy"),
        Ok(DataValue::String("undefined".into()))
    );
}

// Parse-recursion hostile inputs. Boa's parser, bytecode compiler, and AST
// drop recurse natively once per nesting level, so unguarded each of these
// would abort the whole process with `fatal runtime error: stack overflow`.
// They must be declined with a typed error before Boa sees them, on every
// parse path.

/// 12 000 unbracketed `if(1)` statements: statement recursion.
fn if_chain() -> String {
    format!("{}x;", "if(1)".repeat(12_000))
}

/// 4 000 chained conditionals inside the expression byte bound.
fn conditional_chain() -> String {
    format!("{}a", "a?a:".repeat(4_000))
}

/// A quote inside a block comment must not switch off nesting accounting.
fn comment_quote() -> String {
    format!("/*'*/ {}1{}", "(".repeat(3_000), ")".repeat(3_000))
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

#[test]
fn syntax_check_declines_hostile_parse_recursion_without_stack_overflow() {
    use scxml_data_model_ecmascript::{SyntaxKind, check_syntax};
    assert_eq!(
        check_syntax(SyntaxKind::Script, &if_chain()),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        check_syntax(SyntaxKind::Condition, &conditional_chain()),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        check_syntax(SyntaxKind::Condition, &comment_quote()),
        Err(DataModelError::InvalidInput)
    );
}

#[test]
fn runtime_declines_hostile_parse_recursion_without_stack_overflow() {
    let mut model = model();
    let event = Event::external(EventName::new("e").expect("static event name is valid"));
    assert_eq!(
        model.execute_without_event(&if_chain(), &mut NoStates),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.execute(&if_chain(), &event, &mut NoStates),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.evaluate_condition(&conditional_chain(), &event, &mut NoStates),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.evaluate_condition_without_event(&comment_quote(), &mut NoStates),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.evaluate_value(&comment_quote(), &event, &mut NoStates),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.evaluate_value_without_event(&conditional_chain(), &mut NoStates),
        Err(DataModelError::InvalidInput)
    );
}
