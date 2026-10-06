//! `Math.random` (ECMA-262 3rd edition 15.8.2.14): "Returns a number value
//! with positive sign, greater than or equal to 0 but less than 1, chosen
//! randomly or pseudo randomly with approximately uniform distribution over
//! that range, using an implementation-dependent algorithm or strategy."
//!
//! The data model's strategy is a deterministic pseudo-random sequence
//! seeded from the session identity (`_sessionid`): a session draws the
//! same numbers whenever it runs the same operations, so batch runs and
//! replays reproduce, and sessions with different ids draw different
//! numbers.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::DataValue;

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn model(session_id: &str) -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["draws"],
            SystemContext {
                session_id: session_id.into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    model
}

/// Draws through reads, assignments, scripts, and a failed operation.
fn run(model: &mut EcmaScriptDataModel) -> Vec<String> {
    let host = &mut NoStates;
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(format!(
            "{:?}",
            model.evaluate_value_without_event("Math.random()", host)
        ));
    }
    model
        .execute_without_event("draws = [Math.random(), Math.random()]", host)
        .expect("a script draws");
    seen.push(format!(
        "{:?}",
        model.evaluate_value_without_event("draws", host)
    ));
    assert_eq!(
        model.execute_without_event("draws = Math.random(); missing()", host),
        Err(DataModelError::Execution)
    );
    seen.push(format!(
        "{:?}",
        model.evaluate_value_without_event("[Math.random(), draws.length]", host)
    ));
    seen
}

#[test]
fn math_random_is_a_standard_function_in_range() {
    let model = model("s1");
    assert_eq!(
        model.evaluate_value_without_event(
            "(function () { for (var i = 0; i < 500; i++) { var r = Math.random(); \
             if (typeof r !== 'number' || !(r >= 0 && r < 1)) return false; } \
             return typeof Math.random === 'function' && Math.random.length === 0; })()",
            &mut NoStates
        ),
        Ok(DataValue::Bool(true))
    );
}

#[test]
fn the_sequence_is_reproducible_per_session_and_advances_across_operations() {
    let first = run(&mut model("s1"));
    assert_eq!(first, run(&mut model("s1")), "the same session reproduces");
    assert_ne!(first, run(&mut model("s2")), "another session draws others");
    let unique = first.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), first.len(), "operations advance: {first:?}");
}

#[test]
fn the_sequence_is_approximately_uniform() {
    let buckets = model("uniform").evaluate_value_without_event(
        "(function () { var b = [0, 0, 0, 0]; for (var i = 0; i < 4000; i++) \
         b[Math.floor(Math.random() * 4)]++; return b; })()",
        &mut NoStates,
    );
    let Ok(DataValue::JsonArray(text)) = buckets else {
        panic!("{buckets:?}");
    };
    let counts: Vec<u32> = serde_json::from_str(&text).expect("counts");
    assert!(
        counts.iter().all(|count| (850..1150).contains(count)),
        "{counts:?}"
    );
}
