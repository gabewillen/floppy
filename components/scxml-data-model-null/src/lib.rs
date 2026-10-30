//! Null SCXML data-model component domain logic.
//!
//! The Wasm adapter exports the canonical `stateforward:scxml/data-model` WIT
//! interface and rejects every operation outside the null-data-model subset:
//! the `In()` condition (answered by the imported execution host,
//! [`NullDataModel::evaluate_condition`]); SCXML B.1 defines no value,
//! location, or script language.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    use std::cell::RefCell;

    wit_bindgen::generate!({
        world: "data-model",
        path: "../../crates/scxml-wit/wit/scxml",
    });

    use exports::stateforward::scxml::data_model_api::{
        DataModelSnapshot, EffectFinding, EffectFragment, ExternalScripts, Guest, SyntaxIssue,
        SyntaxKind, SystemValues,
    };
    use stateforward::scxml::types::{
        AssignPlacement, AssignSource, BaggagePolicy, ComponentError, DataInitializer, DataValue,
        ErrorCode, ErrorVersion, Event, ForeachResult, InlineBody, NamedValue, TraceContext,
    };
    use stateforward::scxml::{execution_host, telemetry};

    thread_local! {
        // The system values `initialize` installs; `None` until then. The
        // Component Model instance is the single owner: a guest runs on one
        // thread, and no state crosses threads or instances. Each export
        // borrows it for that call only, and imports cannot re-enter it.
        static SYSTEM: RefCell<Option<SystemValues>> = const { RefCell::new(None) };
    }

    struct Component;

    /// The imported `execution-host` configuration query.
    struct ImportedHost;

    impl super::ExecutionHost for ImportedHost {
        fn in_state(&mut self, state_id: &str) -> Result<bool, super::NullDataModelError> {
            Ok(execution_host::in_state(state_id))
        }
    }

    impl Guest for Component {
        fn name() -> String {
            "null".into()
        }

        fn check_syntax(
            _kind: SyntaxKind,
            _source: String,
        ) -> Result<Vec<SyntaxIssue>, ComponentError> {
            // Syntax checking is an optional capability; this provider
            // declines it explicitly instead of claiming validity.
            observed(telemetry::Operation::CheckSyntax, None, || {
                Err(error(
                    ErrorCode::Unavailable,
                    "the null data model has no expression syntax to check",
                ))
            })
        }

        fn check_effects(
            _fragments: Vec<EffectFragment>,
            _scripts: ExternalScripts,
        ) -> Result<Vec<EffectFinding>, ComponentError> {
            // Like syntax checking, effect analysis is an optional
            // capability this provider declines explicitly.
            observed(telemetry::Operation::CheckEffects, None, || {
                Err(error(
                    ErrorCode::Unavailable,
                    "the null data model has no expressions to analyze",
                ))
            })
        }

        fn initialize(
            declarations: Vec<String>,
            context: SystemValues,
        ) -> Result<(), ComponentError> {
            observed(telemetry::Operation::Initialize, None, || {
                validate_system(&context)?;
                // SCXML B.1: the null data model has no data locations.
                if !declarations.is_empty() {
                    return Err(error(
                        ErrorCode::InvalidInput,
                        "the null data model declares no data",
                    ));
                }
                SYSTEM.with_borrow_mut(|system| *system = Some(context));
                Ok(())
            })
        }

        fn declare(_id: String) -> Result<(), ComponentError> {
            observed(telemetry::Operation::Assign, None, || {
                require_initialized()?;
                Err(no_locations())
            })
        }

        fn initialize_data(
            _id: String,
            initializer: DataInitializer,
        ) -> Result<(), ComponentError> {
            observed(telemetry::Operation::Assign, None, || {
                if let DataInitializer::Value(value) = &initializer {
                    validate_value(value)?;
                }
                require_initialized()?;
                Err(no_locations())
            })
        }

        fn set_parameter(_name: String, value: DataValue) -> Result<bool, ComponentError> {
            observed(telemetry::Operation::Assign, None, || {
                validate_value(&value)?;
                require_initialized()?;
                Err(no_locations())
            })
        }

        fn evaluate_location(
            _location: String,
            event: Option<Event>,
        ) -> Result<DataValue, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateValue,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    Err(no_location_expressions())
                },
            )
        }

        fn evaluate_text(
            _expression: String,
            event: Option<Event>,
        ) -> Result<Option<String>, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateValue,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    Err(no_value_expressions())
                },
            )
        }

        fn evaluate_log(
            _expression: String,
            event: Option<Event>,
        ) -> Result<String, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateValue,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    Err(no_value_expressions())
                },
            )
        }

        fn evaluate_form_value(
            _source: String,
            _location: bool,
            event: Option<Event>,
        ) -> Result<DataValue, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateValue,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    Err(no_value_expressions())
                },
            )
        }

        // The null data model has no `<foreach>` collections to track.
        fn release_foreach() {}

        fn foreach_items(
            _expression: String,
            event: Option<Event>,
        ) -> Result<ForeachResult, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateValue,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    // SCXML B.1.4 "There is no value expression language.",
                    // so `array` never evaluates to a legal iterable
                    // collection (4.6: the processor "MUST terminate
                    // execution of the <foreach> element and the block that
                    // contains it, and place the error error.execution on
                    // the internal event queue").
                    Err(no_value_expressions())
                },
            )
        }

        // SCXML B.1: no namelist locations, payload values, inline values,
        // or external data exist in the null data model.
        fn namelist_key(_location: String) -> Option<String> {
            None
        }

        fn event_payload(_entries: Vec<NamedValue>) -> Option<DataValue> {
            None
        }

        fn inline_content(_content: InlineBody) -> Option<DataValue> {
            None
        }

        fn external_data(_text: String) -> Option<DataValue> {
            None
        }

        fn event_data_field(_data: DataValue, _name: String) -> Option<DataValue> {
            None
        }

        // SCXML B.1.6: "System variables are not accessible.", so the data
        // of an event is never read and every format is handled.
        fn accepts_event_data(_data: DataValue) -> bool {
            true
        }

        fn evaluate_condition(
            expression: String,
            event: Option<Event>,
        ) -> Result<bool, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateCondition,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    // SCXML B.1: the only condition is `In()`, answered by
                    // the imported execution-host configuration query.
                    super::NullDataModel
                        .evaluate_condition(&expression, &mut ImportedHost)
                        .map_err(|_| {
                            error(
                                ErrorCode::InvalidInput,
                                "the null data model's only condition is In('state-id')",
                            )
                        })
                },
            )
        }

        fn evaluate_value(
            _expression: String,
            event: Option<Event>,
        ) -> Result<DataValue, ComponentError> {
            observed_context(
                telemetry::Operation::EvaluateValue,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    Err(no_value_expressions())
                },
            )
        }

        fn assign(
            _location: String,
            source: AssignSource,
            _placement: AssignPlacement,
            event: Option<Event>,
            context: Option<TraceContext>,
        ) -> Result<(), ComponentError> {
            let validated_context = (|| {
                validated_event(event.as_ref())?;
                if let AssignSource::Value(value) | AssignSource::ForeachValue(value) = &source {
                    validate_value(value)?;
                }
                validated_raw_context(context.as_ref())
            })()
            .map_err(evaluation_input);
            observed_context(telemetry::Operation::Assign, validated_context, || {
                require_initialized()?;
                // SCXML B.1.3: "There is no location expression language.",
                // and B.1.7: <assign> (section 5) "not supported in the Null
                // Data Model".
                Err(no_location_expressions())
            })
        }

        fn execute(_script: String, event: Option<Event>) -> Result<(), ComponentError> {
            observed_context(
                telemetry::Operation::Execute,
                validated_event(event.as_ref()),
                || {
                    require_initialized()?;
                    // SCXML B.1.5: "There is no scripting language."
                    Err(error(
                        ErrorCode::Execution,
                        "the null data model has no scripting language",
                    ))
                },
            )
        }

        fn system_context() -> Result<SystemValues, ComponentError> {
            observed(telemetry::Operation::SystemContext, None, || {
                SYSTEM.with_borrow(|system| {
                    system.clone().ok_or_else(|| {
                        error(ErrorCode::Unavailable, "data model is not initialized")
                    })
                })
            })
        }

        // Read-only: the retained system values and no variables (SCXML
        // B.1: no data locations), so never an `oversized` one; records no
        // telemetry observation.
        fn snapshot() -> Result<DataModelSnapshot, ComponentError> {
            let system = SYSTEM
                .with_borrow(Clone::clone)
                .ok_or_else(|| error(ErrorCode::Unavailable, "data model is not initialized"))?;
            Ok(DataModelSnapshot {
                system,
                variables: Vec::new(),
            })
        }
    }

    /// SCXML B.1.4: "There is no value expression language." 5.9.3: a
    /// value expression that returns no legal data value is
    /// `error.execution`.
    fn no_value_expressions() -> ComponentError {
        error(
            ErrorCode::Execution,
            "the null data model has no value expression language",
        )
    }

    /// SCXML B.1.3: "There is no location expression language." 5.9.2: a
    /// location that cannot be evaluated is `error.execution`.
    fn no_location_expressions() -> ComponentError {
        error(
            ErrorCode::Execution,
            "the null data model has no location expression language",
        )
    }

    /// SCXML B.1: the null data model has no data locations.
    fn no_locations() -> ComponentError {
        error(
            ErrorCode::Unavailable,
            "the null data model has no data locations",
        )
    }

    fn require_initialized() -> Result<(), ComponentError> {
        let initialized = SYSTEM.with_borrow(Option::is_some);
        initialized
            .then_some(())
            .ok_or_else(|| error(ErrorCode::Unavailable, "data model is not initialized"))
    }

    fn observed<T>(
        operation: telemetry::Operation,
        context: Option<&TraceContext>,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        let result = body();
        telemetry::observe(operation, classify(&result), None, context);
        telemetry::event(
            operation,
            if result.is_ok() {
                telemetry::SignalEvent::TransformationCompleted
            } else {
                telemetry::SignalEvent::InputRejected
            },
            context,
        );
        result
    }

    fn classify<T>(result: &Result<T, ComponentError>) -> telemetry::Outcome {
        match result {
            Ok(_) => telemetry::Outcome::Ok,
            Err(error) if is_rejected(error.code) => telemetry::Outcome::Rejected,
            Err(_) => telemetry::Outcome::Failed,
        }
    }

    fn observed_context<T>(
        operation: telemetry::Operation,
        context: Result<Option<TraceContext>, ComponentError>,
        body: impl FnOnce() -> Result<T, ComponentError>,
    ) -> Result<T, ComponentError> {
        match context {
            Ok(context) => observed(operation, context.as_ref(), body),
            Err(error) => observed(operation, None, || Err(error)),
        }
    }

    /// An evaluation's input over a byte bound is `invalid-input`, as in the
    /// bundled ECMAScript and `XPath` providers: the machine then fails that
    /// evaluation with `error.execution` (a condition is false, SCXML 5.9)
    /// instead of failing the session as it does for `payload-too-large`.
    fn evaluation_input(error: ComponentError) -> ComponentError {
        if error.code == ErrorCode::PayloadTooLarge {
            ComponentError {
                code: ErrorCode::InvalidInput,
                ..error
            }
        } else {
            error
        }
    }

    /// Bounds `input` and, when present, `_event`; `none` means SCXML defines
    /// no current event, so there is nothing further to validate. Over a
    /// bound is `invalid-input` ([`evaluation_input`]).
    fn validated_event(event: Option<&Event>) -> Result<Option<TraceContext>, ComponentError> {
        validated_event_bounds(event).map_err(evaluation_input)
    }

    fn validated_event_bounds(
        event: Option<&Event>,
    ) -> Result<Option<TraceContext>, ComponentError> {
        let Some(event) = event else {
            return Ok(None);
        };
        scxml_types::EventName::new(event.name.clone())
            .map_err(|_| error(ErrorCode::InvalidInput, "event name is invalid"))?;
        if let Some(value) = event.send_id.as_deref() {
            scxml_types::SendId::new(value.to_owned())
                .map_err(|_| error(ErrorCode::InvalidInput, "send id is invalid"))?;
        }
        if let Some(value) = event.invoke_id.as_deref() {
            scxml_types::InvokeId::new(value.to_owned())
                .map_err(|_| error(ErrorCode::InvalidInput, "invoke id is invalid"))?;
        }
        validate_value(&event.data)?;
        validated_raw_context(event.trace_context.as_ref())
    }

    fn validated_raw_context(
        context: Option<&TraceContext>,
    ) -> Result<Option<TraceContext>, ComponentError> {
        context
            .map(|context| {
                bounded(&context.traceparent, scxml_types::TRACEPARENT_BYTES)?;
                if let Some(value) = context.tracestate.as_deref() {
                    bounded(value, scxml_types::MAX_TRACESTATE_BYTES)?;
                }
                if let Some(value) = context.baggage.as_deref() {
                    bounded(value, scxml_types::MAX_BAGGAGE_BYTES)?;
                }
                scxml_types::TraceContext::new(
                    context.traceparent.clone(),
                    context.tracestate.clone(),
                    context.baggage.clone(),
                    match context.baggage_policy {
                        BaggagePolicy::Drop => scxml_types::BaggagePolicy::Drop,
                        BaggagePolicy::Propagate => scxml_types::BaggagePolicy::Propagate,
                    },
                )
                .map_err(|_| error(ErrorCode::InvalidInput, "trace context is invalid"))
                .map(|validated| TraceContext {
                    traceparent: validated.traceparent,
                    tracestate: validated.tracestate,
                    baggage: validated.baggage,
                    baggage_policy: match validated.baggage_policy {
                        scxml_types::BaggagePolicy::Drop => BaggagePolicy::Drop,
                        scxml_types::BaggagePolicy::Propagate => BaggagePolicy::Propagate,
                    },
                })
            })
            .transpose()
    }

    fn validate_system(value: &SystemValues) -> Result<(), ComponentError> {
        if value.session_id.is_empty() || value.machine_name.as_ref().is_some_and(String::is_empty)
        {
            return Err(error(ErrorCode::InvalidInput, "system value is invalid"));
        }
        validate_value(&value.io_processors)?;
        if !matches!(value.io_processors, DataValue::JsonObject(_)) {
            return Err(error(ErrorCode::InvalidInput, "system value is invalid"));
        }
        Ok(())
    }

    fn validate_value(value: &DataValue) -> Result<(), ComponentError> {
        match value {
            DataValue::JsonArray(value) => {
                if scxml_types::DataValue::canonical_json_array(value)
                    == Ok(scxml_types::DataValue::JsonArray(value.clone()))
                {
                    Ok(())
                } else {
                    Err(error(ErrorCode::InvalidInput, "data value is invalid"))
                }
            }
            DataValue::JsonObject(value) => {
                if scxml_types::DataValue::canonical_json_object(value)
                    == Ok(scxml_types::DataValue::JsonObject(value.clone()))
                {
                    Ok(())
                } else {
                    Err(error(ErrorCode::InvalidInput, "data value is invalid"))
                }
            }
            DataValue::Number(value) if !value.is_finite() => {
                Err(error(ErrorCode::InvalidInput, "data value is invalid"))
            }
            DataValue::Absent
            | DataValue::Undefined
            | DataValue::Null
            | DataValue::Boolean(_)
            | DataValue::Number(_)
            | DataValue::Text(_)
            | DataValue::Xml(_) => Ok(()),
        }
    }

    fn bounded(value: &str, max: usize) -> Result<(), ComponentError> {
        if value.len() <= max {
            Ok(())
        } else {
            Err(error(
                ErrorCode::PayloadTooLarge,
                "input exceeds byte bound",
            ))
        }
    }

    fn error(code: ErrorCode, detail: &'static str) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code,
            detail: (detail.len() <= 160).then(|| detail.into()),
        }
    }

    fn is_rejected(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::InvalidInput
                | ErrorCode::DuplicateId
                | ErrorCode::UnknownId
                | ErrorCode::PayloadTooLarge
                | ErrorCode::UnsupportedVersion
                | ErrorCode::Protocol
        )
    }

    export!(Component);
}

/// Bounded null data-model failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NullDataModelError {
    /// The expression or state is outside the null data model or over bound.
    InvalidInput,
    /// The execution host's `In()` state query failed.
    Host,
}

impl std::fmt::Display for NullDataModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "null data-model input is outside the null data model",
            Self::Host => "execution-host state query failed",
        })
    }
}

impl std::error::Error for NullDataModelError {}

/// Explicit execution-host capability the null data model's `In()` uses.
pub trait ExecutionHost {
    /// Whether `state_id` is in the current state configuration.
    ///
    /// # Errors
    ///
    /// Returns [`NullDataModelError::Host`] when the host query fails.
    fn in_state(&mut self, state_id: &str) -> Result<bool, NullDataModelError>;
}

/// Stateless null data model (SCXML B.1).
///
/// Its only condition is the `In('state-id')` predicate (B.1.2). It
/// evaluates no value, location, or script: B.1.3 "There is no location
/// expression language.", B.1.4 "There is no value expression language.",
/// B.1.5 "There is no scripting language."
#[derive(Default)]
pub struct NullDataModel;

impl NullDataModel {
    /// Returns the W3C data-model name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        "null"
    }

    /// Evaluates the null data model's only condition, `In('state-id')`
    /// (SCXML B.1): true iff the host reports the state active.
    ///
    /// # Errors
    ///
    /// [`NullDataModelError::InvalidInput`] for an expression other than one
    /// `In()` call with one quoted state id; [`NullDataModelError::Host`]
    /// when the query fails.
    pub fn evaluate_condition(
        &self,
        expression: &str,
        host: &mut impl ExecutionHost,
    ) -> Result<bool, NullDataModelError> {
        let span = scxml_telemetry::observe(
            scxml_telemetry::Module::DataModelNullComponent,
            scxml_telemetry::Operation::NullDataModelEvaluateCondition,
        );
        let result = in_predicate_state(expression).and_then(|state_id| host.in_state(state_id));
        complete(&span, &result);
        result
    }
}

/// Records one boundary outcome on `span`.
fn complete<T>(span: &scxml_telemetry::FunctionSpan, result: &Result<T, NullDataModelError>) {
    match result {
        Ok(_) => span.event(scxml_telemetry::SignalEvent::TransformationCompleted),
        Err(NullDataModelError::InvalidInput) => {
            span.event(scxml_telemetry::SignalEvent::InputRejected);
            span.fail(scxml_telemetry::ErrorCategory::InvalidInput);
        }
        Err(NullDataModelError::Host) => {
            span.event(scxml_telemetry::SignalEvent::InputRejected);
            span.fail(scxml_telemetry::ErrorCategory::Execution);
        }
    }
}

/// The state id of an expression that is exactly one `In('state-id')`
/// call, optionally surrounded by XML white space (#x20, #x9, #xD, #xA).
fn in_predicate_state(expression: &str) -> Result<&str, NullDataModelError> {
    let argument = expression
        .trim_matches(scxml_types::is_xml_space)
        .strip_prefix("In")
        .map(|rest| rest.trim_start_matches(scxml_types::is_xml_space))
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or(NullDataModelError::InvalidInput)?;
    let state_id = quoted(argument.trim_matches(scxml_types::is_xml_space))?;
    if state_id.is_empty() || state_id.contains(scxml_types::is_xml_space) {
        return Err(NullDataModelError::InvalidInput);
    }
    Ok(state_id)
}

/// The text of one `'...'` or `"..."` literal with no escapes, no inner
/// quote of the same kind, and no line breaks.
fn quoted(literal: &str) -> Result<&str, NullDataModelError> {
    let text = literal
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .filter(|text| !text.contains('\''))
        .or_else(|| {
            literal
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .filter(|text| !text.contains('"'))
        })
        .ok_or(NullDataModelError::InvalidInput)?;
    if text.contains(['\\', '\n', '\r']) {
        return Err(NullDataModelError::InvalidInput);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Configuration(&'static [&'static str]);

    impl ExecutionHost for Configuration {
        fn in_state(&mut self, state_id: &str) -> Result<bool, NullDataModelError> {
            Ok(self.0.contains(&state_id))
        }
    }

    struct FailingHost;

    impl ExecutionHost for FailingHost {
        fn in_state(&mut self, _state_id: &str) -> Result<bool, NullDataModelError> {
            Err(NullDataModelError::Host)
        }
    }

    #[test]
    fn in_predicate_reports_exact_configuration_membership() {
        let mut host = Configuration(&["p", "ps0", "ps1"]);
        assert_eq!(
            NullDataModel.evaluate_condition("In('ps1')", &mut host),
            Ok(true)
        );
        assert_eq!(
            NullDataModel.evaluate_condition(" In ( \"ps0\" ) ", &mut host),
            Ok(true)
        );
        assert_eq!(
            NullDataModel.evaluate_condition("In('s1')", &mut host),
            Ok(false)
        );
        // XML white space only: U+1680 is a name character.
        assert_eq!(
            NullDataModel
                .evaluate_condition("In('a\u{1680}b')", &mut Configuration(&["a\u{1680}b"])),
            Ok(true)
        );
        for rejected in [
            "",
            "true",
            "In(s1)",
            "In('')",
            "In('a b')",
            "!In('p')",
            "In('p') && In('ps0')",
            "In('p'",
            "in('p')",
            "\u{a0}In('p')",
            "In('p')\u{1680}",
        ] {
            assert_eq!(
                NullDataModel.evaluate_condition(rejected, &mut host),
                Err(NullDataModelError::InvalidInput),
                "{rejected:?} is not the null data model's In() predicate"
            );
        }
        assert_eq!(
            NullDataModel.evaluate_condition("In('p')", &mut FailingHost),
            Err(NullDataModelError::Host)
        );
    }

    /// A condition has no byte bound of its own.
    #[test]
    fn long_conditions_evaluate() {
        let padded = format!("In('p'){}", " ".repeat(1_048_576));
        assert_eq!(
            NullDataModel.evaluate_condition(&padded, &mut Configuration(&["p"])),
            Ok(true)
        );
    }
}
