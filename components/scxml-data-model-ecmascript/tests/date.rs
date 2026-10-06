//! `Date` (ECMA-262 3rd edition 15.9), required by B.2: "Implementations
//! that support this value MUST support the third edition of ECMAScript".
//!
//! The current time is the session clock's (the execution host's
//! `now_ms`: the logical clock of the session's delayed sends, so batch
//! runs and replays read the same time and a live session its elapsed
//! time): ECMA-262 15.9.3.3 `new Date()` is "the time value (UTC)
//! identifying the current time", and 15.9.1.9 leaves the local time zone
//! adjustment to the implementation, which is UTC here.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{DataValue, FiniteNumber};

/// A host whose session clock reads `now` milliseconds, counting reads.
struct Clock {
    now: u64,
    reads: usize,
}

impl ExecutionHost for Clock {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }

    fn now_ms(&mut self) -> Result<u64, DataModelError> {
        self.reads += 1;
        Ok(self.now)
    }
}

struct Broken;

impl ExecutionHost for Broken {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }

    fn now_ms(&mut self) -> Result<u64, DataModelError> {
        Err(DataModelError::Host)
    }
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["d", "t"],
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    model
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("finite"))
}

#[test]
fn date_is_the_standard_constructor() {
    let model = model();
    let host = &mut Clock { now: 0, reads: 0 };
    for guard in [
        "typeof Date === 'function' && new Date(0).getTime() === 0",
        "Date.UTC(2000, 0, 2) === 946771200000",
        "new Date(Date.UTC(2020, 1, 29)).getUTCDate() === 29",
        "new Date(86400000).toISOString() === '1970-01-02T00:00:00.000Z'",
        "new Date(0).getTimezoneOffset() === 0 && new Date(0).getHours() === 0",
        "isNaN(new Date('not a date').getTime())",
        "Date.parse('1970-01-01T00:00:01Z') === 1000",
    ] {
        assert_eq!(
            model.evaluate_condition_without_event(guard, host),
            Ok(true),
            "{guard}"
        );
    }
    assert_eq!(host.reads, 0, "no expression above reads the current time");
}

#[test]
fn the_current_time_is_the_session_clock() {
    let mut model = model();
    let host = &mut Clock {
        now: 1_234_567,
        reads: 0,
    };
    assert_eq!(
        model.evaluate_value_without_event("Date.now()", host),
        Ok(number(1_234_567.0))
    );
    assert_eq!(
        model.evaluate_value_without_event("new Date().getTime() - Date.now()", host),
        Ok(number(0.0))
    );
    model
        .execute_without_event("d = new Date(); t = Date.now();", host)
        .expect("the script reads the clock");
    host.now = 9_000_000;
    assert_eq!(
        model.evaluate_value_without_event("[d.getTime(), t, Date.now()]", host),
        Ok(DataValue::JsonArray("[1234567,1234567,9000000]".into())),
        "a stored date keeps its time; the clock moved on"
    );
    assert_eq!(
        model.evaluate_condition_without_event("d instanceof Date", host),
        Ok(true)
    );
}

#[test]
fn a_host_clock_failure_fails_the_operation() {
    let model = model();
    assert_eq!(
        model.evaluate_value_without_event("Date.now()", &mut Broken),
        Err(DataModelError::Host)
    );
    assert_eq!(
        model.evaluate_value_without_event("new Date(5).getTime()", &mut Broken),
        Ok(number(5.0)),
        "an explicit time reads no clock"
    );
}
