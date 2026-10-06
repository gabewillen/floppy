//! SCXML 1.0 section 5.10: "The Processor MUST place all
//! platform-specific system variables underneath it" (the `_x` root), and
//! "Platforms MUST place all platform-specific system variables under the
//! '_x' root."
//!
//! The data model defines no platform-specific system variable, so the
//! realm's global object carries no adapter name at all: the bounds the
//! built-in guard enforces and the DOM builder for XML values stay inside
//! the adapter's closures, before and after an XML value is used.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{DataInitializer, DataValue, Event, EventName};

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
            ["doc", "names"],
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    model
}

/// Every own global name beginning with `_` except the SCXML system
/// variables.
const ADAPTER_NAMES: &str = "Object.getOwnPropertyNames(globalThis).filter((name) => \
     name.startsWith('_') && !['_event', '_sessionid', '_name', '_ioprocessors'].includes(name))";

#[test]
fn the_global_object_carries_no_adapter_name() {
    let mut model = model();
    let host = &mut NoStates;
    assert_eq!(
        model.evaluate_value_without_event(ADAPTER_NAMES, host),
        Ok(DataValue::JsonArray("[]".into())),
        "before any XML value"
    );
    model
        .initialize_data(
            "doc",
            &DataInitializer::Value(DataValue::Xml("<r><a>1</a></r>".into())),
            host,
        )
        .expect("an XML value initializes");
    let mut event = Event::external(EventName::new("e").expect("valid event name"));
    event.data = DataValue::Xml("<p>2</p>".into());
    assert_eq!(
        model.evaluate_value(
            &format!("[{ADAPTER_NAMES}, doc.documentElement.textContent, _event.data.documentElement.textContent]"),
            &event,
            host
        ),
        Ok(DataValue::JsonArray(r#"[[],"1","2"]"#.into())),
        "after XML values are built"
    );
    assert_eq!(
        model.evaluate_log_without_event("doc", host),
        Ok("<r><a>1</a></r>".into()),
        "an XML value still logs as its text"
    );
    assert_eq!(
        model.evaluate_value_without_event("doc", host),
        Ok(DataValue::Xml("<r><a>1</a></r>".into())),
        "an XML value still converts back to its text"
    );
}
